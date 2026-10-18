' corpus-source: generated
' corpus-flagged: no
' corpus-allow-files: yes
' corpus-prints: 1,2
' corpus-prints: x
Sub Main()
    Dim f As Integer, a As Long, b As Long, s As String
    f = FreeFile
    Open ThisWorkbook.Path & "\data\w.txt" For Output As #f
    Write #f, 1, 2
    Write #f, "x"
    Close #f
    f = FreeFile
    Open ThisWorkbook.Path & "\data\w.txt" For Input As #f
    Line Input #f, s
    Debug.Print s
    Input #f, s
    Debug.Print s
    Close #f
End Sub
