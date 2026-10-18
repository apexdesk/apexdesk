' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Title
' corpus-prints: True
Sub Main()
    Range("A1").Value = "Title"
    Range("A1:D1").Merge
    Range("A1").HorizontalAlignment = xlCenter
    Debug.Print Range("A1").Value
    Debug.Print Range("B1").MergeCells
End Sub
