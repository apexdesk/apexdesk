' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim r As Range, n As Long
    Set r = ActiveDocument.Content
    With r.Find
        .Text = "The"
        .MatchCase = True
        Do While .Execute
            n = n + 1
            r.Collapse wdCollapseEnd
        Loop
    End With
    Debug.Print n
End Sub
