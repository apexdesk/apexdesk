' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
Sub Main()
    Debug.Print (Dir(ThisWorkbook.Path & "\data\input.txt") <> "")
    Debug.Print (Dir(ThisWorkbook.Path & "\data\nothing.txt") <> "")
End Sub
