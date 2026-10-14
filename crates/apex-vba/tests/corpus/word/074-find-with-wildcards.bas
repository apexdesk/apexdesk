' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Content
    With r.Find
        .Text = "qu*k"
        .MatchWildcards = True
        Debug.Print IIf(.Execute, 1, 0)
    End With
End Sub
