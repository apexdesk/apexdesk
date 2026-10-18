' corpus-source: generated
' corpus-flagged: no
' corpus-prints: alpha
' corpus-prints: beta
' corpus-prints: gamma
' corpus-prints: 3
Sub Main()
    Dim f As Integer, s As String, n As Long
    f = FreeFile
    Open ThisWorkbook.Path & "\data\input.txt" For Input As #f
    Do While Not EOF(f)
        Line Input #f, s
        n = n + 1
        Cells(n, 1).Value = s
        Debug.Print s
    Loop
    Close #f
    Debug.Print n
End Sub
