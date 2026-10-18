' corpus-expect: blocked
' corpus-source: generated (sandbox probe)
' corpus-allow-files: yes
Sub Main()
    On Error Resume Next
    Dim fso As Object
    Set fso = CreateObject("Scripting.FileSystemObject")
    fso.DeleteFile "%OUTSIDE%\secret.txt"
    fso.CopyFile "%OUTSIDE%\secret.txt", ThisWorkbook.Path & "\"
    fso.MoveFolder "%OUTSIDE%\private", ThisWorkbook.Path & "\p"
    fso.CreateTextFile("%OUTSIDE%\new.txt").WriteLine "x"
End Sub
