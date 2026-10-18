' corpus-kind: word
' corpus-source: generated
' corpus-expect: ok
' corpus-flagged: no
' corpus-prints: 18
' corpus-prints: -1
Sub Main()
    With ActiveDocument.Styles("Heading 1")
        .Font.Size = 18
        .Font.Bold = True
    End With
    Debug.Print ActiveDocument.Styles("Heading 1").Font.Size
    Debug.Print ActiveDocument.Styles("Heading 1").Font.Bold
End Sub
