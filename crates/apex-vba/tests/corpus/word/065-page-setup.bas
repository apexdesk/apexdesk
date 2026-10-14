' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    With ActiveDocument.PageSetup
        .Orientation = wdOrientLandscape
        .TopMargin = 72
        .BottomMargin = 72
    End With
    Debug.Print ActiveDocument.PageSetup.Orientation
End Sub
