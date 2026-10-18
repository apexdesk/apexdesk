' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Apples
' corpus-prints: 5
Sub Main()
    Dim f As Integer, ln As String, parts() As String, r As Long
    f = FreeFile
    Open ThisWorkbook.Path & "\data\table.csv" For Input As #f
    Do While Not EOF(f)
        Line Input #f, ln
        r = r + 1
        parts = Split(ln, ",")
        Cells(r, 1).Value = parts(0)
        If r > 1 Then Cells(r, 2).Value = CLng(parts(1)) Else Cells(r, 2).Value = parts(1)
    Loop
    Close #f
    Debug.Print Range("A2").Value
    Debug.Print Range("B3").Value
End Sub
