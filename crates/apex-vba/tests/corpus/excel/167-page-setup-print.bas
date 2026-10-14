' corpus-source: generated
' corpus-flagged: no
' corpus-prints: done
Sub Main()
    With ActiveSheet.PageSetup
        .Orientation = xlLandscape
        .PrintArea = "$A$1:$D$10"
        .CenterFooter = "Page &P"
    End With
    Debug.Print "done"
End Sub
