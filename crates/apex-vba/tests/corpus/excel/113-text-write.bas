' corpus-source: generated
' corpus-flagged: no
' corpus-allow-files: yes
' corpus-prints: line one
' corpus-prints: line two
Sub Main()
    Dim f As Integer, s As String, p As String
    p = ThisWorkbook.Path & "\data\out.txt"
    f = FreeFile
    Open p For Output As #f
    Print #f, "line one"
    Print #f, "line two"
    Close #f
    f = FreeFile
    Open p For Input As #f
    Do While Not EOF(f)
        Line Input #f, s
        Debug.Print s
    Loop
    Close #f
End Sub
