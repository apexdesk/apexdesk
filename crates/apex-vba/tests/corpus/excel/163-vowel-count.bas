' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 7
Sub Main()
    Dim s As String, i As Long, n As Long
    s = "Programming Language"
    For i = 1 To Len(s)
        If InStr("aeiouAEIOU", Mid(s, i, 1)) > 0 Then n = n + 1
    Next i
    Debug.Print n
End Sub
