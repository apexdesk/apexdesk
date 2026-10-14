' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Arial
' corpus-prints: 12
' corpus-prints: True
' corpus-prints: True
Sub Main()
    With Range("A1").Font
        .Name = "Arial"
        .Size = 12
        .Italic = True
        .Underline = xlUnderlineStyleSingle
    End With
    Debug.Print Range("A1").Font.Name
    Debug.Print Range("A1").Font.Size
    Debug.Print Range("A1").Font.Italic
    Debug.Print (Range("A1").Font.Underline = xlUnderlineStyleSingle)
End Sub
