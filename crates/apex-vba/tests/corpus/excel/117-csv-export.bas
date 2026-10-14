' corpus-source: generated
' corpus-flagged: no
' corpus-allow-files: yes
' corpus-prints: A,B,C
' corpus-prints: 1,2,3
Sub Main()
    Dim f As Integer, r As Long, c As Long, ln As String, s As String
    For c = 1 To 3
        Cells(1, c).Value = Chr(64 + c)
        Cells(2, c).Value = c
    Next c
    f = FreeFile
    Open ThisWorkbook.Path & "\data\export.csv" For Output As #f
    For r = 1 To 2
        ln = ""
        For c = 1 To 3
            ln = ln & Cells(r, c).Value
            If c < 3 Then ln = ln & ","
        Next c
        Print #f, ln
    Next r
    Close #f
    f = FreeFile
    Open ThisWorkbook.Path & "\data\export.csv" For Input As #f
    Do While Not EOF(f)
        Line Input #f, s
        Debug.Print s
    Loop
    Close #f
End Sub
