' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim s As String, n As Long, i As Long
    s = ActiveDocument.Content.Text
    For i = 1 To Len(s)
        If Mid(s, i, 1) = "." Then n = n + 1
    Next i
    Debug.Print n
End Sub
