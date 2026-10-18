' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 20
' corpus-prints: 30
Sub Main()
    Columns("A").ColumnWidth = 20
    Rows(1).RowHeight = 30
    Debug.Print Columns("A").ColumnWidth
    Debug.Print Rows(1).RowHeight
End Sub
