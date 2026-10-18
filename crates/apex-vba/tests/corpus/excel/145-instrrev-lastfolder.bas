' corpus-source: generated
' corpus-flagged: no
' corpus-prints: file.txt
' corpus-prints: txt
Sub Main()
    Dim p As String
    p = "a\b\file.txt"
    Debug.Print Mid(p, InStrRev(p, "\") + 1)
    Debug.Print Mid(p, InStrRev(p, ".") + 1)
End Sub
