' corpus-source: generated
' corpus-flagged: no
' corpus-allow-files: yes
' corpus-prints: 3
Sub Main()
    Dim f As Integer, n As Long, s As String
    f = FreeFile
    Open ThisWorkbook.Path & "\data\log.txt" For Append As #f
    Print #f, "start"
    Print #f, "middle"
    Close #f
    f = FreeFile
    Open ThisWorkbook.Path & "\data\log.txt" For Append As #f
    Print #f, "end"
    Close #f
    f = FreeFile
    Open ThisWorkbook.Path & "\data\log.txt" For Input As #f
    Do While Not EOF(f)
        Line Input #f, s
        n = n + 1
    Loop
    Close #f
    Debug.Print n
End Sub
