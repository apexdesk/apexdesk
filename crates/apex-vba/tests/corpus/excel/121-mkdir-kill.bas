' corpus-source: generated
' corpus-flagged: no
' corpus-allow-files: yes
' corpus-prints: True
' corpus-prints: False
Sub Main()
    Dim d As String, f As Integer
    d = ThisWorkbook.Path & "\temp_out"
    MkDir d
    f = FreeFile
    Open d & "\a.txt" For Output As #f
    Print #f, "x"
    Close #f
    Debug.Print (Dir(d & "\a.txt") <> "")
    Kill d & "\a.txt"
    Debug.Print (Dir(d & "\a.txt") <> "")
    RmDir d
End Sub
