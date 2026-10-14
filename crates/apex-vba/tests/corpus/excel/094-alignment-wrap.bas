' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: -4108
Sub Main()
    With Range("A1")
        .Value = "Long wrapped text"
        .WrapText = True
        .HorizontalAlignment = xlCenter
        .VerticalAlignment = xlTop
    End With
    Debug.Print Range("A1").WrapText
    Debug.Print Range("A1").HorizontalAlignment
End Sub
