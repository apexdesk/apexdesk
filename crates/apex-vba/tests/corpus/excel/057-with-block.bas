' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Title
' corpus-prints: 14
Sub Main()
    With Range("A1")
        .Value = "Title"
        .Font.Bold = True
        .Font.Size = 14
    End With
    Debug.Print Range("A1").Value
    Debug.Print Range("A1").Font.Size
End Sub
