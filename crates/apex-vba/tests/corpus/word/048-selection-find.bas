' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: lazy
Sub Main()
    Selection.HomeKey wdStory
    With Selection.Find
        .Text = "lazy"
        .Execute
    End With
    Debug.Print Selection.Text
End Sub
