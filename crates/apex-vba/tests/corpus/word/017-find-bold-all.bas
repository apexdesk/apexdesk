' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: -1
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Content
    With r.Find
        .Text = "quick"
        If .Execute Then r.Font.Bold = True
    End With
    Debug.Print r.Font.Bold
End Sub
