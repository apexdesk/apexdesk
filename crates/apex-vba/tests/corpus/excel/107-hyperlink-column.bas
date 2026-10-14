' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim i As Long
    For i = 1 To 3
        Cells(i, 1).Value = "page" & i
        ActiveSheet.Hyperlinks.Add Anchor:=Cells(i, 1), Address:="https://example.org/page" & i
    Next i
    Debug.Print ActiveSheet.Hyperlinks.Count
End Sub
